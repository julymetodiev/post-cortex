// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Reloads and breakers: the running set across a reload, and a failing
//! module refused until its breaker closes.

use super::*;

#[tokio::test]
async fn a_reload_keeps_running_modules_starts_new_ones_and_stops_the_rest() {
    let kept: Arc<dyn Module> = Arc::new(tool(Act::Answer));
    let gone = fake(
        "gone",
        ModuleKind::Tool,
        Act::Answer,
        FailurePolicy::FailOpen,
    );
    let gone_stopped = Arc::clone(&gone.stopped);
    let host = Host::start(
        vec![Arc::clone(&kept), Arc::new(gone)],
        &HashMap::new(),
        &HashMap::new(),
    )
    .await
    .unwrap();
    host.call_for_agent(&scope(Duration::from_secs(1)), "tool_echo", Value::Null)
        .await
        .unwrap();

    let new = fake(
        "new",
        ModuleKind::Tool,
        Act::Answer,
        FailurePolicy::FailOpen,
    );
    host.reload(
        vec![Arc::clone(&kept), Arc::new(new)],
        &HashMap::new(),
        &HashMap::new(),
    )
    .await
    .unwrap();
    assert!(gone_stopped.load(std::sync::atomic::Ordering::SeqCst));
    assert!(host.has_tool("new_echo") && !host.has_tool("gone_echo"));
    // The kept module is the same instance: its health carries on.
    let health = host.health();
    assert_eq!((health[0].module.as_str(), health[0].calls), ("tool", 1));
}

#[tokio::test]
async fn a_failed_reload_keeps_the_running_set() {
    let host = host(vec![tool(Act::Answer)]).await.unwrap();
    let mut broken = fake(
        "broken",
        ModuleKind::Tool,
        Act::Answer,
        FailurePolicy::FailOpen,
    );
    broken.fails_start = true;
    let refused = host
        .reload(vec![Arc::new(broken)], &HashMap::new(), &HashMap::new())
        .await;
    assert!(matches!(refused, Err(HostError::StartFailed { .. })));
    assert!(host.has_tool("tool_echo"));
}

#[tokio::test]
async fn a_running_module_whose_new_settings_are_refused_says_so_until_they_are_applied() {
    // A cloud saves a module's settings and its tenant reloads later: the
    // module's own refusal was in the server's log only, while the person
    // read "saved" (found 2026-11-14).
    let old: Arc<dyn Module> = Arc::new(tool(Act::Answer));
    let host = Host::start(vec![Arc::clone(&old)], &HashMap::new(), &HashMap::new())
        .await
        .unwrap();
    // The same module (its id) with settings its start refuses.
    let mut new = tool(Act::Answer);
    new.fails_start = true;
    let refused = host
        .reload(vec![Arc::new(new)], &HashMap::new(), &HashMap::new())
        .await
        .unwrap_err();
    assert_eq!(refused.refused_module(), Some("tool"));
    let problem = host.health()[0].problem.clone();
    assert!(
        problem.starts_with("its new settings were not applied, and it runs with the ones before")
            && problem.contains("no backend"),
        "{problem}"
    );
    assert!(host.has_tool("tool_echo"), "the one before runs");

    // Settings that are applied end it.
    host.reload(vec![old], &HashMap::new(), &HashMap::new())
        .await
        .unwrap();
    assert_eq!(host.health()[0].problem, "");
    // What is wrong between modules names none of them.
    assert_eq!(HostError::DuplicateId("tool".into()).refused_module(), None);
}

#[tokio::test]
async fn an_organisations_host_goes_on_without_a_module_that_does_not_start() {
    // Found 2026-11-14: a module that refused its saved settings failed
    // the whole load, so an organisation's tenant did not start.
    let named = |id: &str, fails: bool| {
        let mut module = fake(id, ModuleKind::Tool, Act::Answer, FailurePolicy::FailOpen);
        module.fails_start = fails;
        Arc::new(module) as Arc<dyn Module>
    };
    let (config, policies) = (HashMap::new(), HashMap::new());
    let host = Host::start_with(
        vec![named("board", false), named("discord", true)],
        &config,
        &policies,
        None,
        "acme",
    )
    .await
    .unwrap();
    assert!(host.has_tool("board_echo") && !host.has_tool("discord_echo"));
    let left_out = host.left_out();
    assert_eq!(left_out.len(), 1);
    assert_eq!(left_out[0].0, "discord");
    assert!(left_out[0].1.contains("no backend"), "{}", left_out[0].1);

    // Once it starts it runs, and nothing is left out.
    let board = named("board", false);
    let discord = named("discord", false);
    host.reload(
        vec![Arc::clone(&board), Arc::clone(&discord)],
        &HashMap::new(),
        &HashMap::new(),
    )
    .await
    .unwrap();
    assert!(host.has_tool("discord_echo") && host.left_out().is_empty());

    // New settings it refuses: the one that ran stays and says so; it is
    // not left out, and the other module goes on.
    host.reload(
        vec![board, named("discord", true)],
        &HashMap::new(),
        &HashMap::new(),
    )
    .await
    .unwrap();
    assert!(host.has_tool("discord_echo") && host.left_out().is_empty());
    let health = host.health();
    let discord = health.iter().find(|h| h.module == "discord").unwrap();
    assert!(
        discord
            .problem
            .starts_with("its new settings were not applied")
    );
    assert_eq!(
        health.iter().find(|h| h.module == "board").unwrap().problem,
        ""
    );

    // A daemon's host (no organisation) starts all its modules or none.
    let strict = Host::start(
        vec![named("board", false), named("discord", true)],
        &HashMap::new(),
        &HashMap::new(),
    )
    .await;
    assert!(matches!(strict, Err(HostError::StartFailed { .. })));
}

#[tokio::test]
async fn a_failing_module_is_refused_until_its_breaker_closes() {
    let host = host(vec![tool(Act::Fail)]).await.unwrap();
    for _ in 0..5 {
        let failed = host
            .call_for_agent(&scope(Duration::from_secs(1)), "tool_echo", Value::Null)
            .await;
        assert_eq!(failed.unwrap_err().kind, ModuleErrorKind::Unavailable);
    }
    let refused = host
        .call_for_agent(&scope(Duration::from_secs(1)), "tool_echo", Value::Null)
        .await
        .unwrap_err();
    assert!(refused.message.contains("keeps failing"), "{refused}");
    let health = &host.health()[0];
    assert!(health.breaker_open);
    assert_eq!((health.errors, health.refused), (5, 1));
    // Its log says what the host saw (W25).
    let log = host.module_log("tool").unwrap();
    assert_eq!(log.len(), 6, "five failures and the breaker opening");
    assert!(
        log.iter()
            .all(|l| l.source == pcx_module_api::LogSource::Host)
    );
    assert!(log[0].text.contains("failed"), "{}", log[0].text);
    assert!(log[5].text.contains("keeps failing"), "{}", log[5].text);
    assert!(host.module_log("nobody").is_none());
}

#[tokio::test]
async fn a_caller_s_mistakes_do_not_open_the_breaker() {
    let host = host(vec![tool(Act::Answer)]).await.unwrap();
    for _ in 0..10 {
        host.call_for_agent(&scope(Duration::from_secs(1)), "tool_echo", Value::Null)
            .await
            .unwrap();
    }
    assert!(!host.health()[0].breaker_open);
}

#[tokio::test]
async fn modules_start_at_once_and_one_that_fails_stops_the_others() {
    let slow = |id: &str| {
        let mut module = fake(id, ModuleKind::Tool, Act::Answer, FailurePolicy::FailOpen);
        module.starts_in = Duration::from_millis(300);
        Arc::new(module) as Arc<dyn Module>
    };
    let began = std::time::Instant::now();
    let host = Host::start(
        vec![slow("a"), slow("b"), slow("c")],
        &HashMap::new(),
        &HashMap::new(),
    )
    .await
    .unwrap();
    assert!(
        began.elapsed() < Duration::from_millis(600),
        "{:?}",
        began.elapsed()
    );
    assert!(host.has_tool("a_echo") && host.has_tool("c_echo"));

    let started = fake(
        "started",
        ModuleKind::Tool,
        Act::Answer,
        FailurePolicy::FailOpen,
    );
    let stopped = Arc::clone(&started.stopped);
    let mut broken = fake(
        "broken",
        ModuleKind::Tool,
        Act::Answer,
        FailurePolicy::FailOpen,
    );
    broken.fails_start = true;
    let refused = host
        .reload(
            vec![Arc::new(started), Arc::new(broken)],
            &HashMap::new(),
            &HashMap::new(),
        )
        .await;
    assert!(
        matches!(&refused, Err(HostError::StartFailed { module, .. }) if module == "broken"),
        "{refused:?}"
    );
    assert!(stopped.load(std::sync::atomic::Ordering::SeqCst));
    assert!(host.has_tool("a_echo") && !host.has_tool("started_echo"));
}
