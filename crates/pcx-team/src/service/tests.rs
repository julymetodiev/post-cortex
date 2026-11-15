// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The team service over documents held in memory.

use super::*;

use pcx_documents::InMemoryDocuments;
use post_cortex_proto::team::{Authority, Exception, Mode, Trust};

fn agent(name: &str, authority: Authority, harness: bool) -> Agent {
    Agent {
        name: name.into(),
        role: "builds things".into(),
        authority: authority.into(),
        harness,
        skills: vec!["review".into()],
        model: Some(" ".into()),
        modules: Vec::new(),
        tools: Vec::new(),
        reach: Vec::new(),
    }
}

#[tokio::test]
async fn teams_are_made_of_named_members_with_their_authority() {
    let docs =
        InMemoryDocuments::new(Some("julius"), post_cortex_proto::TEAM_DESCRIPTOR_SET).unwrap();
    let teams = Service::new(&docs);
    let dev = teams
        .add_agent(agent("dev", Authority::Decide, true))
        .await
        .unwrap();
    assert_eq!(
        dev.agent.as_ref().unwrap().model,
        None,
        "an empty model is the agent's"
    );
    teams
        .add_agent(agent("julius", Authority::Close, false))
        .await
        .unwrap();
    let again = teams
        .add_agent(agent("dev", Authority::Close, true))
        .await
        .unwrap_err();
    assert_eq!(again.kind, ModuleErrorKind::Conflict);

    let core = teams
        .create_team(
            "core",
            vec!["dev".into(), "julius".into(), "dev".into()],
            "ws",
        )
        .await
        .unwrap();
    assert_eq!(core.team.as_ref().unwrap().members, ["dev", "julius"]);
    let bad = teams
        .create_team("two words", Vec::new(), "ws")
        .await
        .unwrap_err();
    assert_eq!(bad.kind, ModuleErrorKind::InvalidArgument);

    let rosters = teams.rosters(None).await.unwrap();
    assert_eq!(rosters.len(), 1);
    assert_eq!(rosters[0].agents.len(), 2);

    // A member of no team is listed apart, so it can join one.
    teams
        .add_agent(agent("eve", Authority::Discuss, false))
        .await
        .unwrap();
    let listed = teams.list(None, "ws").await.unwrap();
    assert_eq!(listed.teams.len(), 1);
    let unteamed: Vec<&str> = listed
        .unteamed
        .iter()
        .map(|r| r.agent.as_ref().unwrap().name.as_str())
        .collect();
    assert_eq!(unteamed, ["eve"]);
    assert!(
        teams
            .list(Some("core"), "ws")
            .await
            .unwrap()
            .unteamed
            .is_empty()
    );

    let changed = teams
        .update_agent(UpdateAgentRequest {
            name: "dev".into(),
            version: dev.version,
            authority: Some(Authority::Close.into()),
            ..UpdateAgentRequest::default()
        })
        .await
        .unwrap();
    assert_eq!(
        changed.agent.as_ref().unwrap().authority(),
        Authority::Close
    );
    // A member added as an agent of the harness by mistake becomes known
    // by its name, keeping the rest.
    let person = teams
        .update_agent(UpdateAgentRequest {
            name: "dev".into(),
            version: changed.version,
            harness: Some(false),
            ..UpdateAgentRequest::default()
        })
        .await
        .unwrap();
    let person = person.agent.unwrap();
    assert!(!person.harness);
    assert_eq!(person.authority(), Authority::Close);
    let stale = teams
        .update_agent(UpdateAgentRequest {
            name: "dev".into(),
            version: dev.version,
            ..UpdateAgentRequest::default()
        })
        .await
        .unwrap_err();
    assert_eq!(stale.kind, ModuleErrorKind::Conflict);

    let smaller = teams
        .set_member("core", core.version, "julius", true, "ws")
        .await
        .unwrap();
    assert_eq!(smaller.team.as_ref().unwrap().members, ["dev"]);
    let bigger = teams
        .set_member("core", smaller.version, "codex", false, "ws")
        .await
        .unwrap();
    assert_eq!(bigger.team.as_ref().unwrap().members, ["dev", "codex"]);
}

/// A team is supervised at version 0 until a person sets its trust; a new
/// trust keeps the exceptions set, and pausing keeps the trust.
#[tokio::test]
async fn a_team_is_supervised_until_its_trust_is_set() {
    let docs =
        InMemoryDocuments::new(Some("julius"), post_cortex_proto::TEAM_DESCRIPTOR_SET).unwrap();
    let teams = Service::new(&docs);
    teams.create_team("core", Vec::new(), "ws").await.unwrap();
    let first = teams.autonomy("core").await.unwrap();
    assert_eq!(first.version, 0);
    assert_eq!(first.autonomy, Some(crate::autonomy::supervised("core")));
    let rosters = teams.rosters(Some("core")).await.unwrap();
    assert_eq!(rosters[0].autonomy.as_ref(), Some(&first));

    let exceptions = vec![Exception {
        of: "dev".into(),
        trust: Some(Trust {
            mode: Mode::Supervised.into(),
            ..Trust::default()
        }),
    }];
    let full = Trust {
        mode: Mode::Full.into(),
        ..Trust::default()
    };
    let set = teams
        .set_autonomy("core", 0, full.clone(), Some(exceptions.clone()))
        .await
        .unwrap();
    let kept = teams
        .set_autonomy("core", set.version, full.clone(), None)
        .await
        .unwrap();
    assert_eq!(kept.autonomy.as_ref().unwrap().exceptions, exceptions);
    let paused = teams.pause("core", kept.version, true).await.unwrap();
    let now = teams.autonomy("core").await.unwrap();
    assert_eq!(now.version, paused.version);
    assert!(now.autonomy.as_ref().unwrap().paused);
    assert_eq!(now.autonomy.as_ref().unwrap().trust, Some(full));
    assert!(teams.rosters(Some("other")).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_built_in_agent_is_listed_until_a_person_adds_it_to_a_team() {
    let docs =
        InMemoryDocuments::new(Some("julius"), post_cortex_proto::TEAM_DESCRIPTOR_SET).unwrap();
    let teams = Service::new(&docs);
    let core = teams.create_team("core", Vec::new(), "ws").await.unwrap();
    let listed = teams.list(None, "ws").await.unwrap();
    let names: Vec<&str> = listed.builtin.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["workflow-builder", "module-builder", "community"]);

    let added = teams
        .set_member("core", core.version, "workflow-builder", false, "ws")
        .await
        .unwrap();
    assert_eq!(added.team.unwrap().members, ["workflow-builder"]);
    let listed = teams.list(None, "ws").await.unwrap();
    let names: Vec<&str> = listed.builtin.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(
        names,
        ["module-builder", "community"],
        "the workspace has the other now"
    );
    let roster = &listed.teams[0];
    let builder = roster.agents[0].agent.as_ref().unwrap();
    assert!(builder.harness);
    assert_eq!(builder.skills, ["workflow-builder"]);
    assert!(builder.modules.is_empty(), "all its team reaches");
}

#[tokio::test]
async fn a_team_made_with_a_built_in_agents_name_has_the_agent() {
    // Found 2026-11-14: a team created with `community` among its members
    // named a member that was no agent, so no turn was asked of it.
    let docs =
        InMemoryDocuments::new(Some("julius"), post_cortex_proto::TEAM_DESCRIPTOR_SET).unwrap();
    let teams = Service::new(&docs);
    teams
        .create_team("help", vec!["community".into(), "julius".into()], "public")
        .await
        .unwrap();
    let listed = teams.list(None, "public").await.unwrap();
    let agents: Vec<(&str, bool)> = listed.teams[0]
        .agents
        .iter()
        .filter_map(|record| record.agent.as_ref())
        .map(|agent| (agent.name.as_str(), agent.harness))
        .collect();
    assert_eq!(
        agents,
        [("community", true)],
        "a name that is no built-in adds none"
    );
    assert!(listed.builtin.iter().all(|b| b.name != "community"));
}

#[tokio::test]
async fn the_community_agent_reads_memory_of_the_workspace_it_is_added_in() {
    let docs =
        InMemoryDocuments::new(Some("julius"), post_cortex_proto::TEAM_DESCRIPTOR_SET).unwrap();
    let teams = Service::new(&docs);
    let help = teams.create_team("help", Vec::new(), "ws").await.unwrap();
    teams
        .set_member("help", help.version, "community", false, "public")
        .await
        .unwrap();
    let listed = teams.list(None, "public").await.unwrap();
    let community = listed.teams[0].agents[0].agent.as_ref().unwrap();
    assert_eq!(community.reach, ["public"], "no other workspace");
    assert_eq!(community.modules, ["post-cortex.memory"]);
    assert_eq!(
        community.tools,
        ["memory_search", "memory_context"],
        "tools that only read"
    );
}
