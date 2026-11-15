// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The team service: a workspace's teams and members over its documents,
//! the domain's rules applied to what they read and write (ARCHITECTURE
//! section 8). Between the handlers and the documents; it answers the
//! messages of `pcx.team.v1` (W23).

use pcx_documents::{Documents, Item, Typed};
use pcx_module_api::{ModuleError, ModuleErrorKind};
use post_cortex_proto::team::{
    AgentRecord, AutonomyRecord, Roster, RosterList, TeamRecord, UpdateAgentRequest,
};
use prost::{Message, Name};

use crate::autonomy::{self, Autonomy, Exception, Trust};
use crate::domain::{self, Agent, Broken, Kind, Team};

/// A team as the service answers it.
#[must_use]
pub fn team_record(item: Item<Team>) -> TeamRecord {
    TeamRecord {
        id: item.document.id,
        version: item.document.version,
        team: Some(item.data),
        updated_at: item.document.updated_at,
    }
}

/// A member as the service answers it.
#[must_use]
pub fn agent_record(item: Item<Agent>) -> AgentRecord {
    AgentRecord {
        id: item.document.id,
        version: item.document.version,
        agent: Some(item.data),
        updated_at: item.document.updated_at,
    }
}

/// A team's autonomy as it applies, at the version a change of it starts
/// from: the one a person set, else the supervised one at version 0.
fn applied(team: &str, set: Option<Item<Autonomy>>) -> AutonomyRecord {
    set.map_or_else(
        || AutonomyRecord {
            version: 0,
            autonomy: Some(autonomy::supervised(team)),
        },
        |item| AutonomyRecord {
            version: item.document.version,
            autonomy: Some(item.data),
        },
    )
}

fn broken(rule: Broken) -> ModuleError {
    ModuleError::new(ModuleErrorKind::InvalidArgument, rule.to_string())
}

fn missing(kind: Kind, name: &str) -> ModuleError {
    ModuleError::new(
        ModuleErrorKind::NotFound,
        format!("no {} {name} in the workspace", kind.name()),
    )
}

/// A team named `name`, to find it by.
fn named_team(name: &str) -> Team {
    Team {
        name: name.to_string(),
        ..Team::default()
    }
}

fn taken(what: &str, name: &str) -> ModuleError {
    ModuleError::new(
        ModuleErrorKind::Conflict,
        format!("the workspace has a {what} {name} already"),
    )
}

/// The teams of one workspace.
pub struct Service<'a> {
    docs: &'a dyn Documents,
}

impl<'a> Service<'a> {
    /// The teams over `docs`.
    #[must_use]
    pub fn new(docs: &'a dyn Documents) -> Self {
        Self { docs }
    }

    /// A new team of `members` (each a member's name). A built-in agent's
    /// name the workspace (`workspace`) has no agent of adds the agent from
    /// its template first, as [`Service::set_member`] does.
    ///
    /// # Errors
    /// A name breaks a rule; the name is taken; memory failed.
    pub async fn create_team(
        &self,
        name: &str,
        members: Vec<String>,
        workspace: &str,
    ) -> Result<TeamRecord, ModuleError> {
        let name = domain::name(name).map_err(broken)?.to_string();
        let mut kept: Vec<String> = Vec::new();
        for member in members {
            let member = domain::name(member.trim()).map_err(broken)?.to_string();
            if !kept.contains(&member) {
                kept.push(member);
            }
        }
        if self.team(&name).await?.is_some() {
            return Err(taken("team", &name));
        }
        for member in &kept {
            self.built_in(member, workspace).await?;
        }
        let team = Team {
            name,
            members: kept,
        };
        let text = team.name.clone();
        Ok(team_record(self.of(Kind::Team).create(&team, &text).await?))
    }

    /// Every team, or team `only`, with its members' documents and its
    /// autonomy, by name.
    ///
    /// # Errors
    /// Memory failed.
    pub async fn rosters(&self, only: Option<&str>) -> Result<Vec<Roster>, ModuleError> {
        let mut teams: Vec<Item<Team>> = match only {
            Some(name) => {
                self.of(Kind::Team)
                    .all(Some((&named_team(name), &["name"])))
                    .await?
            }
            None => self.of(Kind::Team).all(None).await?,
        };
        teams.sort_by(|a, b| a.data.name.cmp(&b.data.name));
        let agents: Vec<Item<Agent>> = self.of(Kind::Agent).all(None).await?;
        let autonomies: Vec<Item<Autonomy>> = self.of(Kind::Autonomy).all(None).await?;
        Ok(teams
            .into_iter()
            .map(|team| Roster {
                agents: team
                    .data
                    .members
                    .iter()
                    .filter_map(|m| agents.iter().find(|a| &a.data.name == m).cloned())
                    .map(agent_record)
                    .collect(),
                autonomy: Some(applied(
                    &team.data.name,
                    autonomies
                        .iter()
                        .find(|a| a.data.team == team.data.name)
                        .cloned(),
                )),
                team: Some(team_record(team)),
            })
            .collect())
    }

    /// The rosters of the teams, every one or the one named `only`, and,
    /// of every one, the members of no team by name; the built-in agents
    /// as `workspace` would get them.
    ///
    /// # Errors
    /// Memory failed.
    pub async fn list(
        &self,
        only: Option<&str>,
        workspace: &str,
    ) -> Result<RosterList, ModuleError> {
        let teams = self.rosters(only).await?;
        let mut agents: Vec<Item<Agent>> = self.of(Kind::Agent).all(None).await?;
        let builtin = domain::builtins(workspace)
            .into_iter()
            .filter(|b| !agents.iter().any(|a| a.data.name == b.name))
            .collect();
        if only.is_some() {
            return Ok(RosterList {
                teams,
                unteamed: Vec::new(),
                builtin,
            });
        }
        let members: Vec<&str> = teams
            .iter()
            .filter_map(|roster| roster.team.as_ref()?.team.as_ref())
            .flat_map(|team| team.members.iter().map(String::as_str))
            .collect();
        agents.retain(|a| !members.contains(&a.data.name.as_str()));
        agents.sort_by(|a, b| a.data.name.cmp(&b.data.name));
        let unteamed = agents.into_iter().map(agent_record).collect();
        Ok(RosterList {
            teams,
            unteamed,
            builtin,
        })
    }

    /// A new member's document.
    ///
    /// # Errors
    /// Its name or role breaks a rule; the name is taken; memory failed.
    pub async fn add_agent(&self, agent: Agent) -> Result<AgentRecord, ModuleError> {
        let agent = Agent {
            name: domain::name(&agent.name).map_err(broken)?.to_string(),
            role: domain::filled("role", &agent.role).map_err(broken)?,
            model: agent.model.filter(|m| !m.trim().is_empty()),
            ..agent
        };
        if self.agent(&agent.name).await?.is_some() {
            return Err(taken("member", &agent.name));
        }
        let text = agent.role.clone();
        Ok(agent_record(
            self.of(Kind::Agent).create(&agent, &text).await?,
        ))
    }

    /// Change the member `edit` names from its version: what it leaves
    /// out stays; an empty model is the agent's.
    ///
    /// # Errors
    /// No such member; an empty role; it changed since the version
    /// (`Conflict`); memory failed.
    pub async fn update_agent(&self, edit: UpdateAgentRequest) -> Result<AgentRecord, ModuleError> {
        let current = self
            .agent(&edit.name)
            .await?
            .ok_or_else(|| missing(Kind::Agent, &edit.name))?;
        let mut agent = current.data;
        if let Some(role) = &edit.role {
            agent.role = domain::filled("role", role).map_err(broken)?;
        }
        if edit.authority.is_some() {
            agent.set_authority(edit.authority());
        }
        if let Some(skills) = edit.skills {
            agent.skills = skills.names;
        }
        if let Some(model) = edit.model {
            agent.model = Some(model).filter(|m| !m.trim().is_empty());
        }
        if let Some(modules) = edit.modules {
            agent.modules = modules.names;
        }
        if let Some(tools) = edit.tools {
            agent.tools = tools.names;
        }
        if let Some(harness) = edit.harness {
            agent.harness = harness;
        }
        if let Some(reach) = edit.reach {
            agent.reach = reach.names;
        }
        let text = agent.role.clone();
        Ok(agent_record(
            self.of(Kind::Agent)
                .change(&current.document.id, edit.version, &agent, &text)
                .await?,
        ))
    }

    /// The built-in agent named `member` added from its template, when the
    /// workspace (`workspace`) has no agent of that name; nothing for any
    /// other name. A team names its members, and the team runs turns only
    /// of those that are agents: a built-in named without its agent would
    /// be a member no turn is asked of (found 2026-11-14).
    async fn built_in(&self, member: &str, workspace: &str) -> Result<(), ModuleError> {
        if self.agent(member).await?.is_none()
            && let Some(builtin) = domain::builtins(workspace)
                .into_iter()
                .find(|b| b.name == member)
        {
            self.add_agent(builtin).await?;
        }
        Ok(())
    }

    /// Add `member` to team `team` from its `version`, or with `remove`
    /// take it out. A built-in agent's name the workspace (`workspace`) has
    /// no agent of adds the agent from its template first.
    ///
    /// # Errors
    /// No such team; a name that breaks a rule; it changed since `version`
    /// (`Conflict`); memory failed.
    pub async fn set_member(
        &self,
        team: &str,
        version: u64,
        member: &str,
        remove: bool,
        workspace: &str,
    ) -> Result<TeamRecord, ModuleError> {
        let member = domain::name(member).map_err(broken)?;
        let current = self
            .team(team)
            .await?
            .ok_or_else(|| missing(Kind::Team, team))?;
        if !remove {
            self.built_in(member, workspace).await?;
        }
        let mut data = current.data;
        data.members.retain(|m| m != member);
        if !remove {
            data.members.push(member.to_string());
        }
        let text = data.name.clone();
        Ok(team_record(
            self.of(Kind::Team)
                .change(&current.document.id, version, &data, &text)
                .await?,
        ))
    }

    /// Team `team`'s autonomy as it applies.
    ///
    /// # Errors
    /// No such team; memory failed.
    pub async fn autonomy(&self, team: &str) -> Result<AutonomyRecord, ModuleError> {
        Ok(applied(team, self.set_autonomy_of(team).await?))
    }

    /// The autonomy a person set for team `team`, if any.
    async fn set_autonomy_of(&self, team: &str) -> Result<Option<Item<Autonomy>>, ModuleError> {
        self.team(team)
            .await?
            .ok_or_else(|| missing(Kind::Team, team))?;
        let of = Autonomy {
            team: team.to_string(),
            ..Autonomy::default()
        };
        let found: Vec<Item<Autonomy>> =
            self.of(Kind::Autonomy).all(Some((&of, &["team"]))).await?;
        Ok(found.into_iter().next())
    }

    /// Set team `team`'s trust, and its exceptions when given (none: the
    /// ones set stay), from the autonomy's `version` (0: none set yet),
    /// keeping whether it is paused. For a person only (the tool's
    /// visibility).
    ///
    /// # Errors
    /// No such team; an exception that names no one; it changed since
    /// `version` (`Conflict`); memory failed.
    pub async fn set_autonomy(
        &self,
        team: &str,
        version: u64,
        trust: Trust,
        exceptions: Option<Vec<Exception>>,
    ) -> Result<AutonomyRecord, ModuleError> {
        for exception in exceptions.iter().flatten() {
            domain::filled("an exception's member or role", &exception.of).map_err(broken)?;
        }
        self.write_autonomy(team, version, |current| Autonomy {
            trust: Some(trust),
            exceptions: exceptions.unwrap_or(current.exceptions),
            ..current
        })
        .await
    }

    /// Pause team `team`'s autonomy, or with `paused` false go on, from
    /// its `version` (0: none set yet). For a person only.
    ///
    /// # Errors
    /// As [`Service::set_autonomy`].
    pub async fn pause(
        &self,
        team: &str,
        version: u64,
        paused: bool,
    ) -> Result<AutonomyRecord, ModuleError> {
        self.write_autonomy(team, version, |current| Autonomy { paused, ..current })
            .await
    }

    /// Write the autonomy `change` makes of the current one (the
    /// supervised one while none is set), from `version`.
    async fn write_autonomy(
        &self,
        team: &str,
        version: u64,
        change: impl FnOnce(Autonomy) -> Autonomy,
    ) -> Result<AutonomyRecord, ModuleError> {
        let autonomies = self.of(Kind::Autonomy);
        let written = match self.set_autonomy_of(team).await? {
            None if version == 0 => {
                let autonomy = change(autonomy::supervised(team));
                autonomies.create(&autonomy, team).await?
            }
            Some(current) if version > 0 => {
                let autonomy = change(current.data);
                autonomies
                    .change(&current.document.id, version, &autonomy, team)
                    .await?
            }
            Some(current) => {
                return Err(ModuleError::new(
                    ModuleErrorKind::Conflict,
                    format!(
                        "team {team}'s autonomy is at version {}",
                        current.document.version
                    ),
                ));
            }
            None => {
                return Err(ModuleError::new(
                    ModuleErrorKind::Conflict,
                    format!("team {team} has no autonomy yet: set it from version 0"),
                ));
            }
        };
        Ok(applied(team, Some(written)))
    }

    /// Team `name`, if the workspace has it.
    async fn team(&self, name: &str) -> Result<Option<Item<Team>>, ModuleError> {
        self.first(Kind::Team, &named_team(name), "name").await
    }

    /// Member `name`, if the workspace has it.
    async fn agent(&self, name: &str) -> Result<Option<Item<Agent>>, ModuleError> {
        let of = Agent {
            name: name.to_string(),
            ..Agent::default()
        };
        self.first(Kind::Agent, &of, "name").await
    }

    /// The first document of `kind` whose `field` is `example`'s.
    async fn first<T: Message + Name + Default>(
        &self,
        kind: Kind,
        example: &T,
        field: &str,
    ) -> Result<Option<Item<T>>, ModuleError> {
        let found: Vec<Item<T>> = self.of(kind).all(Some((example, &[field]))).await?;
        Ok(found.into_iter().next())
    }

    fn of(&self, kind: Kind) -> Typed<'_> {
        Typed::new(self.docs, kind.document())
    }
}

#[cfg(test)]
mod tests;
