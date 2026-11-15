// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Who writes a turn's prompt (W25): who asks, else the scope's author.

use super::*;

#[tokio::test]
async fn the_prompt_is_written_as_who_asks_and_the_answers_as_the_speaker() {
    let model = Scripted::with(vec![Ok(said("Two tasks."))]);
    let (tools, hooks, kept) = (Echo::default(), Heard::default(), Kept::default());
    let settings = settings();
    let scope = Scope {
        author: Some("chief".into()),
        ..scope()
    };
    let agent = Agent {
        models: &model,
        tools: &tools,
        hooks: &hooks,
        transcript: &kept,
        context: None,
        settings: &settings,
        scope: &scope,
    };
    let (tx, _rx) = mpsc::channel(64);
    let asked = TurnRequest {
        speaker: Some("chief".into()),
        asker: Some("julius".into()),
        ..TurnRequest::new("s1", "Which tasks are open?")
    };
    agent.run(&asked, &tx).await.unwrap();
    let authors: Vec<Option<String>> = kept.turns.lock().iter().map(|t| t.author.clone()).collect();
    assert_eq!(
        authors[0].as_deref(),
        Some("julius"),
        "the person a module's turn is for"
    );
    assert_eq!(authors[1].as_deref(), Some("chief"));

    // Without one, the scope's author asks.
    let (tx, _rx) = mpsc::channel(64);
    agent
        .run(&TurnRequest::new("s2", "And now?"), &tx)
        .await
        .unwrap();
    let last_prompt = kept
        .turns
        .lock()
        .iter()
        .rev()
        .find(|t| t.role == Role::User)
        .map(|t| t.author.clone());
    assert_eq!(last_prompt, Some(Some("chief".into())));
}
