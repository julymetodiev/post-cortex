// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The turns a module runs in its own name (`Manifest.turns`): declared
//! and run together, asked by its id, by a module that uses it.

use super::*;

/// A module that is not the agent, declaring turns (`Manifest.turns`) or
/// not, and running them or not.
fn speaker(declares: bool, runs: bool) -> Arc<Holder> {
    let host = Handle::default();
    let mut manifest = holder("speaker", ModuleKind::Tool, &[]).manifest.clone();
    manifest.turns = declares;
    Arc::new(Holder {
        manifest,
        turns: runs.then(|| {
            Arc::new(AgentTurns {
                host: Arc::clone(&host),
                tokens: Arc::default(),
                reached: Arc::default(),
            })
        }),
        host,
    })
}

#[tokio::test]
async fn a_module_runs_turns_in_its_own_name_only_when_it_declares_them() {
    for (declares, runs) in [(true, false), (false, true)] {
        let refused = Host::start(
            vec![speaker(declares, runs) as Arc<dyn Module>],
            &HashMap::new(),
            &HashMap::new(),
        )
        .await;
        assert!(
            matches!(refused, Err(HostError::InvalidManifest { .. })),
            "declares {declares}, runs {runs}"
        );
    }
    let mut agent = holder("agent", ModuleKind::Agent, &[]);
    Arc::get_mut(&mut agent).unwrap().manifest.turns = true;
    let refused = Host::start(
        vec![agent as Arc<dyn Module>],
        &HashMap::new(),
        &HashMap::new(),
    )
    .await;
    assert!(matches!(refused, Err(HostError::InvalidManifest { .. })));

    // The agent slot's turns are the agent's, whatever loads first; a
    // module's own are asked by its id.
    let speaker = speaker(true, true);
    let agent = holder("agent", ModuleKind::Agent, &[]);
    let host = Host::start(
        vec![speaker.clone() as Arc<dyn Module>, agent.clone()],
        &HashMap::new(),
        &HashMap::new(),
    )
    .await
    .unwrap();
    let scope = scope(Duration::from_secs(5));
    turn(host.turns().unwrap().as_ref(), &scope, "").await;
    assert_eq!(agent.turns.as_ref().unwrap().tokens.lock().len(), 1);
    turn(host.turns_of("speaker").unwrap().as_ref(), &scope, "").await;
    assert_eq!(speaker.turns.as_ref().unwrap().tokens.lock().len(), 1);
    assert!(host.turns_of("agent").is_none());
    assert!(host.turns_of("nobody").is_none());
}

#[tokio::test]
async fn a_module_asks_the_turns_of_a_module_it_uses_and_of_no_other() {
    let speaker = speaker(true, true);
    let agent = holder("agent", ModuleKind::Agent, &[]);
    let asks = holder("asks", ModuleKind::Tool, &["speaker", "agent"]);
    let stranger = holder("stranger", ModuleKind::Tool, &[]);
    let _host = Host::start(
        vec![
            speaker.clone() as Arc<dyn Module>,
            agent.clone(),
            asks.clone(),
            stranger.clone(),
        ],
        &HashMap::new(),
        &HashMap::new(),
    )
    .await
    .unwrap();
    let scope = scope(Duration::from_secs(5));

    // The speaker's own turn, as it runs it: the asking module gives it no
    // token of its own, and the agent is not asked.
    let turns = handle(&asks).turns_of("speaker").await.unwrap();
    turn(turns.as_ref(), &scope, "").await;
    assert_eq!(*speaker.turns.as_ref().unwrap().tokens.lock(), vec![None]);
    assert!(agent.turns.as_ref().unwrap().tokens.lock().is_empty());

    let refused = handle(&stranger).turns_of("speaker").await.err().unwrap();
    assert_eq!(refused.kind, ModuleErrorKind::Denied, "{refused:?}");
    // The agent's turns are the slot's, not its own name's; a module that
    // is not there runs none.
    let none = handle(&asks).turns_of("agent").await.err().unwrap();
    assert_eq!(none.kind, ModuleErrorKind::NotFound, "{none:?}");
    let unused = handle(&asks).turns_of("nobody").await.err().unwrap();
    assert_eq!(unused.kind, ModuleErrorKind::Denied, "{unused:?}");
}
